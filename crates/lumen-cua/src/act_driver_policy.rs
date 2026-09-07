//! Host-written cua-driver policy. Background delivery only.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

pub const MCP_SERVER_NAME: &str = "computer-use";

pub const POLICY_YAML: &str = r#"# Written by Lumen Cua on each ActDriverEnsure. Do not edit by hand.
allow:
  tools:
    - check_permissions
    - health_report
    - get_agent_cursor_state
    - get_config
    - set_config
    - get_screen_size
    - get_window_state
    - get_desktop_state
    - launch_app
    - list_apps
    - list_windows
    - page
    - start_session
    - end_session
    - get_session
    - set_agent_cursor_enabled
    - set_agent_cursor_motion
    - set_agent_cursor_theme
    - set_value
  rules:
    - tool: click
      constraints:
        delivery_mode:
          allowed: [background]
    - tool: right_click
      constraints:
        delivery_mode:
          allowed: [background]
    - tool: type_text
      constraints:
        delivery_mode:
          allowed: [background]
    - tool: press_key
      constraints:
        delivery_mode:
          allowed: [background]
    - tool: hotkey
      constraints:
        delivery_mode:
          allowed: [background]
    - tool: scroll
      constraints:
        delivery_mode:
          allowed: [background]
"#;

pub const MANAGED_POLICY_REGO: &str = r#"# Written by Lumen Cua on each ActDriverEnsure. Do not edit by hand.
package cua.policy

import rego.v1

default allow := false

allow if {
    input.tool != "hotkey"
}

allow if {
    input.tool == "hotkey"
    not address_bar_shortcut
}

address_bar_shortcut if {
    keys := [lower(key) | some key in input.arguments.keys]
    some modifier in keys
    modifier in {"cmd", "command", "meta", "ctrl", "control"}
    "l" in keys
}
"#;

pub fn write_policy_files(policy_path: &Path, managed_path: &Path) -> Result<()> {
    if let Some(parent) = policy_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(policy_path, POLICY_YAML)
        .with_context(|| format!("write {}", policy_path.display()))?;
    fs::write(managed_path, MANAGED_POLICY_REGO)
        .with_context(|| format!("write {}", managed_path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_locks_input_tools_to_background() {
        assert!(POLICY_YAML.contains("delivery_mode"));
        assert!(POLICY_YAML.contains("allowed: [background]"));
        assert!(POLICY_YAML.contains("launch_app"));
        assert!(POLICY_YAML.contains("page"));
        assert!(POLICY_YAML.contains("set_agent_cursor_enabled"));
        assert!(!POLICY_YAML.contains("bring_to_front"));
        assert!(!POLICY_YAML.contains("move_cursor"));
    }

    #[test]
    fn managed_policy_blocks_address_bar_hotkey() {
        assert!(MANAGED_POLICY_REGO.contains("address_bar_shortcut"));
        assert!(MANAGED_POLICY_REGO.contains("\"l\" in keys"));
    }

    #[test]
    fn write_policy_files_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let policy = dir.path().join("driver-policy.yaml");
        let managed = dir.path().join("driver-managed-policy.rego");
        write_policy_files(&policy, &managed).unwrap();
        assert_eq!(fs::read_to_string(&policy).unwrap(), POLICY_YAML);
        assert_eq!(fs::read_to_string(&managed).unwrap(), MANAGED_POLICY_REGO);
    }
}
