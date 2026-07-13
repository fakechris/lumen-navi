use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use crate::NativeBrowserBridgeConfig;
use serde::{Deserialize, Serialize};

const PROTOCOL_VERSION: u32 = 1;

#[derive(Serialize)]
struct HostHello<'a> {
    kind: &'static str,
    protocol_version: u32,
    owner: &'a str,
    origin: &'a str,
    token: &'a str,
}

#[derive(Deserialize)]
struct HostAck {
    kind: String,
    ok: bool,
}

pub fn run_native_browser_host() -> Result<(), String> {
    run_native_browser_host_with_config(None)
}

pub fn run_native_browser_host_with_config(
    default_config_path: Option<PathBuf>,
) -> Result<(), String> {
    let (config_path, origin) = arguments(default_config_path)?;
    let config: NativeBrowserBridgeConfig = serde_json::from_slice(
        &std::fs::read(&config_path).map_err(|error| format!("read config: {error}"))?,
    )
    .map_err(|error| format!("decode config: {error}"))?;
    config.validate().map_err(|error| error.to_string())?;
    if !config.allowed_origins.contains(&origin) {
        return Err("extension origin is not allowlisted".to_owned());
    }
    let token = std::fs::read_to_string(&config.token_path)
        .map_err(|error| format!("read auth token: {error}"))?;
    let token = token.trim();
    if token.is_empty() {
        return Err("auth token is empty".to_owned());
    }
    let mut socket = UnixStream::connect(&config.socket_path)
        .map_err(|error| format!("connect app socket: {error}"))?;
    let hello = serde_json::to_vec(&HostHello {
        kind: "hello",
        protocol_version: PROTOCOL_VERSION,
        owner: &config.owner,
        origin: &origin,
        token,
    })
    .map_err(|error| format!("encode hello: {error}"))?;
    write_bridge_frame(&mut socket, &hello, config.max_message_bytes)?;
    let ack: HostAck =
        serde_json::from_slice(&read_bridge_frame(&mut socket, config.max_message_bytes)?)
            .map_err(|error| format!("decode hello response: {error}"))?;
    if ack.kind != "hello_ack" || !ack.ok {
        return Err("app rejected native host authentication".to_owned());
    }

    let mut extension_to_app = socket
        .try_clone()
        .map_err(|error| format!("clone app socket: {error}"))?;
    let limit = config.max_message_bytes;
    let input = std::thread::spawn(move || -> Result<(), String> {
        let mut stdin = std::io::stdin().lock();
        while let Ok(message) = read_native_frame(&mut stdin, limit) {
            write_bridge_frame(&mut extension_to_app, &message, limit)?;
        }
        Ok(())
    });

    let mut stdout = std::io::stdout().lock();
    while let Ok(message) = read_bridge_frame(&mut socket, limit) {
        write_native_frame(&mut stdout, &message, limit)?;
    }
    let _ = input.join();
    Ok(())
}

fn arguments(default_config_path: Option<PathBuf>) -> Result<(PathBuf, String), String> {
    let mut config_path = None;
    let mut origin = None;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--config" {
            config_path = arguments.next().map(PathBuf::from);
        } else if let Some(value) = argument.strip_prefix("--config=") {
            config_path = Some(PathBuf::from(value));
        } else if argument.starts_with("chrome-extension://")
            || argument.starts_with("safari-web-extension://")
        {
            origin = Some(argument);
        }
    }
    let config_path = match config_path {
        Some(path) => path,
        None => default_config_path.unwrap_or(
            std::env::current_exe()
                .map_err(|error| format!("locate host executable: {error}"))?
                .with_extension("browser-host.json"),
        ),
    };
    let origin = origin.ok_or_else(|| "browser did not provide an extension origin".to_owned())?;
    Ok((config_path, origin))
}

fn read_native_frame(reader: &mut impl Read, limit: usize) -> Result<Vec<u8>, String> {
    let mut length = [0_u8; 4];
    reader
        .read_exact(&mut length)
        .map_err(|error| format!("read extension frame: {error}"))?;
    read_body(reader, u32::from_ne_bytes(length) as usize, limit)
}

fn write_native_frame(writer: &mut impl Write, message: &[u8], limit: usize) -> Result<(), String> {
    validate_length(message.len(), limit)?;
    writer
        .write_all(&(message.len() as u32).to_ne_bytes())
        .and_then(|_| writer.write_all(message))
        .and_then(|_| writer.flush())
        .map_err(|error| format!("write extension frame: {error}"))
}

fn read_bridge_frame(reader: &mut impl Read, limit: usize) -> Result<Vec<u8>, String> {
    let mut length = [0_u8; 4];
    reader
        .read_exact(&mut length)
        .map_err(|error| format!("read app frame: {error}"))?;
    read_body(reader, u32::from_be_bytes(length) as usize, limit)
}

fn write_bridge_frame(writer: &mut impl Write, message: &[u8], limit: usize) -> Result<(), String> {
    validate_length(message.len(), limit)?;
    writer
        .write_all(&(message.len() as u32).to_be_bytes())
        .and_then(|_| writer.write_all(message))
        .and_then(|_| writer.flush())
        .map_err(|error| format!("write app frame: {error}"))
}

fn read_body(reader: &mut impl Read, length: usize, limit: usize) -> Result<Vec<u8>, String> {
    validate_length(length, limit)?;
    let mut message = vec![0_u8; length];
    reader
        .read_exact(&mut message)
        .map_err(|error| format!("read frame body: {error}"))?;
    Ok(message)
}

fn validate_length(length: usize, limit: usize) -> Result<(), String> {
    if length == 0 || length > limit {
        Err("native message length is invalid".to_owned())
    } else {
        Ok(())
    }
}
