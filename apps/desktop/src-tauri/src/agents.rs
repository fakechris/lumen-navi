//! ProcessAgentRunner — run local CLI agents (claude / codex / …) as sandboxed
//! subprocesses and stream their stdout into the same `assistant-stream`
//! event channel the HTTP path uses (frontend stays protocol-agnostic).
//!
//! Discipline (mirrors the Atat template approach):
//! - templates live in navi.toml `[agents]`, disabled by default
//! - command must contain `{prompt}`; the prompt lands as one argv element
//! - sandbox flags are part of the template itself (`--safe-mode`,
//!   `--sandbox read-only`, `--ephemeral`, read-only tool whitelists)
//! - stdout is streamed verbatim; stderr is captured for the error path

use lumen_config::AgentTemplate;
use serde_json::json;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use tauri::{AppHandle, Emitter};

use crate::selection_popup::POPUP_LABEL;

/// Info about one runnable agent for the popup selector.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentInfo {
    /// "http" or the template id.
    pub id: String,
    pub label: String,
}

/// Agents available right now: the configured HTTP model (always first) plus
/// every enabled template whose executable resolves on PATH.
pub fn list_available(cfg: &lumen_config::Config) -> Vec<AgentInfo> {
    let mut out = vec![AgentInfo {
        id: "http".into(),
        label: format!("{}（配置的模型）", cfg.assistant.model),
    }];
    for t in &cfg.agents.templates {
        if !t.enabled {
            continue;
        }
        if let Some(bin) = first_token(&t.command) {
            if which(&bin).is_some() {
                out.push(AgentInfo {
                    id: t.id.clone(),
                    label: t.label.clone(),
                });
            }
        }
    }
    out
}

/// Find a template by id.
pub fn template_by_id(cfg: &lumen_config::Config, id: &str) -> Option<AgentTemplate> {
    cfg.agents.templates.iter().find(|t| t.id == id).cloned()
}

/// Validate + expand a template into argv, substituting `{prompt}`.
/// Returns Err with a user-facing message on invalid templates.
pub fn expand_template(t: &AgentTemplate, prompt: &str) -> Result<Vec<String>, String> {
    if !t.command.contains("{prompt}") {
        return Err(format!("agent 模板 {} 缺少 {{prompt}} 占位符", t.id));
    }
    if prompt.trim().is_empty() {
        return Err("空 prompt".into());
    }
    // Advisory only — log, never block (user-owned templates).
    if !t.command.contains("--sandbox")
        && !t.command.contains("--safe-mode")
        && !t.command.contains("--permission-mode")
    {
        tracing::warn!(
            agent = %t.id,
            "agent template carries no sandbox flags (--sandbox/--safe-mode/--permission-mode)"
        );
    }
    Ok(t.command
        .split_whitespace()
        .map(|tok| if tok == "{prompt}" { prompt.to_string() } else { tok.to_string() })
        .collect())
}

/// Run one agent subprocess to completion, emitting each stdout line as an
/// `assistant-stream` delta. Errors carry stderr (truncated).
pub fn run_process_agent(
    app: &AppHandle,
    job_id: &str,
    template: &AgentTemplate,
    prompt: &str,
) -> Result<(), String> {
    let argv = expand_template(template, prompt)?;
    let (bin, args) = argv.split_first().ok_or("empty agent command")?;

    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("启动 {} 失败：{e}（是否已安装并在 PATH？）", template.id))?;

    let stderr = child.stderr.take();
    let stderr_thread = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(mut e) = stderr {
            use std::io::Read;
            let _ = e.read_to_string(&mut buf);
        }
        buf
    });

    if let Some(stdout) = child.stdout.take() {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    let delta = line.trim_end_matches('\n');
                    if !delta.is_empty() {
                        let _ = app.emit_to(
                            POPUP_LABEL,
                            "assistant-stream",
                            json!({ "id": job_id, "delta": format!("{delta}\n") }),
                        );
                    }
                }
                Err(e) => {
                    let err = stderr_thread.join().unwrap_or_default();
                    return Err(format!("读取 {} 输出失败：{e}{}", template.id, tail(&err)));
                }
            }
        }
    }

    let status = child
        .wait()
        .map_err(|e| format!("等待 {} 退出失败：{e}", template.id))?;
    let err = stderr_thread.join().unwrap_or_default();
    if !status.success() {
        return Err(format!(
            "{} 退出码 {:?}{}",
            template.id,
            status.code(),
            tail(&err)
        ));
    }
    Ok(())
}

fn first_token(command: &str) -> Option<String> {
    command.split_whitespace().next().map(str::to_string)
}

/// Minimal `which`: PATH lookup for an executable. None when not found.
fn which(bin: &str) -> Option<std::path::PathBuf> {
    if bin.contains('/') {
        let p = std::path::Path::new(bin);
        return p.is_file().then(|| p.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(bin))
        .find(|p| p.is_file())
}

fn tail(s: &str) -> String {
    let cut: String = s.chars().take(300).collect();
    if cut.is_empty() {
        String::new()
    } else {
        format!("\n{cut}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumen_config::AgentsConfig;

    #[test]
    fn expand_replaces_prompt_as_one_arg() {
        let t = &AgentsConfig::default().templates[0];
        let argv = expand_template(t, "hello world").unwrap();
        assert!(argv.contains(&"hello world".to_string()));
        assert!(!argv.iter().any(|a| a == "{prompt}"));
    }

    #[test]
    fn expand_rejects_missing_placeholder() {
        let t = AgentTemplate {
            id: "x".into(),
            label: "X".into(),
            command: "echo hi".into(),
            enabled: true,
        };
        assert!(expand_template(&t, "p").is_err());
    }

    #[test]
    fn expand_rejects_empty_prompt() {
        let t = &AgentsConfig::default().templates[0];
        assert!(expand_template(t, "  ").is_err());
    }

    #[test]
    fn default_templates_carry_sandbox_flags() {
        for t in &AgentsConfig::default().templates {
            let c = &t.command;
            assert!(
                c.contains("--sandbox") || c.contains("--safe-mode") || c.contains("--permission-mode"),
                "{} missing sandbox flags",
                t.id
            );
            assert!(!t.enabled, "{} should default off", t.id);
        }
    }
}
