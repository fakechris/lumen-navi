//! Selection-popup assistant — OpenAI-compatible streaming chat client.
//!
//! Sends the user-selected text to `{base_url}/chat/completions` (SSE) and
//! forwards deltas to the `selection-popup` window as Tauri events:
//! `assistant-stream` {id, delta}; the caller emits `assistant-done` /
//! `assistant-error`. Runs only on explicit user action from the popup.

use futures_util::StreamExt;
use lumen_config::AssistantConfig;
use serde_json::json;
use tauri::{AppHandle, Emitter};

use crate::selection_popup::POPUP_LABEL;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssistantAction {
    Translate,
    Ask,
}

impl AssistantAction {
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "translate" => Ok(Self::Translate),
            "ask" => Ok(Self::Ask),
            other => Err(format!("unknown assistant action: {other}")),
        }
    }
}

#[derive(Debug)]
pub struct AssistantJob {
    pub id: String,
    pub action: AssistantAction,
    pub text: String,
    pub question: Option<String>,
}

/// Chat messages for an action (pure, unit-testable).
pub fn build_messages(
    action: AssistantAction,
    cfg: &AssistantConfig,
    text: &str,
    question: Option<&str>,
) -> Vec<serde_json::Value> {
    match action {
        AssistantAction::Translate => vec![
            json!({
                "role": "system",
                "content": format!(
                    "You are a translation engine. Translate the user text into {}. \
                     Output only the translation — no explanations, no quotes, \
                     preserve the original formatting and line breaks.",
                    cfg.target_lang
                ),
            }),
            json!({ "role": "user", "content": text }),
        ],
        AssistantAction::Ask => {
            let q = question.unwrap_or("").trim();
            vec![
                json!({
                    "role": "system",
                    "content": "You are the Lumen Navi assistant. The user selected a \
                        piece of text as context; answer their question based on it. \
                        Be concise and reply in the same language as the question.",
                }),
                json!({
                    "role": "user",
                    "content": format!("上下文:\n\"\"\"\n{text}\n\"\"\"\n\n问题:{q}"),
                }),
            ]
        }
    }
}

/// Stream one completion; emits `assistant-stream` deltas to the popup window.
/// Terminal events (`assistant-done` / `assistant-error`) are the caller's job.
pub async fn run_stream(
    app: AppHandle,
    cfg: AssistantConfig,
    job: AssistantJob,
) -> Result<(), String> {
    if cfg.base_url.trim().is_empty() || cfg.model.trim().is_empty() {
        return Err("assistant base_url / model not configured".into());
    }
    let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
    let messages = build_messages(job.action, &cfg, &job.text, job.question.as_deref());
    let body = json!({
        "model": cfg.model,
        "messages": messages,
        "stream": true,
        "temperature": 0.3,
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(cfg.timeout_ms.max(5_000)))
        .build()
        .map_err(|e| format!("http client: {e}"))?;

    let mut request = client.post(&url).json(&body);
    let key = cfg.effective_api_key();
    if !key.is_empty() {
        request = request.bearer_auth(key);
    }
    let resp = request
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let snippet = resp.text().await.unwrap_or_default();
        let snippet: String = snippet.chars().take(300).collect();
        return Err(format!("LLM HTTP {status}: {snippet}"));
    }

    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    'outer: while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("stream read: {e}"))?;
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(pos) = buf.find('\n') {
            let line = buf[..pos].trim_end_matches('\r').to_string();
            buf.drain(..=pos);
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() {
                continue;
            }
            if data == "[DONE]" {
                break 'outer;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };
            let delta = v
                .get("choices")
                .and_then(|c| c.as_array())
                .and_then(|a| a.first())
                .and_then(|c| c.get("delta"))
                .and_then(|d| d.get("content"))
                .and_then(|c| c.as_str())
                .unwrap_or("");
            if !delta.is_empty() {
                let _ = app.emit_to(
                    POPUP_LABEL,
                    "assistant-stream",
                    json!({ "id": job.id, "delta": delta }),
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translate_messages_use_target_lang() {
        let mut cfg = AssistantConfig::default();
        cfg.target_lang = "English".into();
        let msgs = build_messages(AssistantAction::Translate, &cfg, "你好", None);
        assert_eq!(msgs.len(), 2);
        assert!(msgs[0]["content"].as_str().unwrap().contains("English"));
        assert_eq!(msgs[1]["content"], "你好");
    }

    #[test]
    fn ask_messages_embed_context_and_question() {
        let cfg = AssistantConfig::default();
        let msgs = build_messages(AssistantAction::Ask, &cfg, "段文字", Some("什么意思?"));
        let user = msgs[1]["content"].as_str().unwrap();
        assert!(user.contains("段文字"));
        assert!(user.contains("什么意思?"));
    }

    #[test]
    fn parse_action() {
        assert_eq!(AssistantAction::parse("Translate").unwrap(), AssistantAction::Translate);
        assert!(AssistantAction::parse("nope").is_err());
    }
}
