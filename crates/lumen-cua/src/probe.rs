//! Read-only L0 probe. Never relaunches the target app.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Result};

use crate::protocol::ProbeReport;

pub fn probe_app(name_or_path: &str) -> Result<ProbeReport> {
    let path = resolve_app(name_or_path)?;
    let plist = path.join("Contents/Info.plist");
    let bundle_id = plist_string(&plist, "CFBundleIdentifier");
    let version = plist_string(&plist, "CFBundleShortVersionString")
        .or_else(|| plist_string(&plist, "CFBundleVersion"));
    let display_name = mdls_display_name(&path).or_else(|| {
        plist_string(&plist, "CFBundleDisplayName").or_else(|| plist_string(&plist, "CFBundleName"))
    });
    let architecture = detect_architecture(&path);
    let url_schemes = url_schemes(&plist);
    let applescript_enabled = plist_bool(&plist, "NSAppleScriptEnabled");
    let sdef = plist_string(&plist, "OSAScriptingDefinition").map(|rel| {
        let candidate = path.join("Contents/Resources").join(&rel);
        if candidate.exists() {
            candidate.display().to_string()
        } else {
            rel
        }
    });
    let pid = pid_for_bundle(bundle_id.as_deref());
    let (listen_ports, cdp_likely) = listen_ports(pid);
    let duplicate_installs = bundle_id
        .as_deref()
        .map(duplicate_installs)
        .unwrap_or_default();

    Ok(ProbeReport {
        path: path.display().to_string(),
        display_name,
        bundle_id,
        version,
        architecture,
        url_schemes,
        applescript_enabled,
        sdef_path: sdef,
        listen_ports,
        cdp_likely,
        duplicate_installs,
        ax_editable_count: None,
        pid,
    })
}

fn resolve_app(name_or_path: &str) -> Result<PathBuf> {
    let trimmed = name_or_path.trim();
    if trimmed.is_empty() {
        bail!("empty app name");
    }
    let as_path = PathBuf::from(trimmed);
    if as_path.is_dir() && as_path.extension().and_then(|e| e.to_str()) == Some("app") {
        return Ok(as_path);
    }
    let applications = PathBuf::from("/Applications").join(format!("{trimmed}.app"));
    if applications.is_dir() {
        return Ok(applications);
    }
    if let Some(found) = mdfind_app(trimmed) {
        return Ok(found);
    }
    bail!("app not found: {trimmed}")
}

fn mdfind_app(name: &str) -> Option<PathBuf> {
    let query = format!(
        "kMDItemContentType == 'com.apple.application-bundle' && kMDItemDisplayName == '{name}*'c"
    );
    let output = Command::new("/usr/bin/mdfind").arg(query).output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| line.ends_with(".app"))
        .map(PathBuf::from)
}

fn plist_string(plist: &Path, key: &str) -> Option<String> {
    let output = Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", &format!("Print :{key}"), &plist.display().to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() || s == "Does Not Exist" {
        None
    } else {
        Some(s)
    }
}

fn plist_bool(plist: &Path, key: &str) -> bool {
    matches!(
        plist_string(plist, key).as_deref(),
        Some("true") | Some("1") | Some("YES")
    )
}

fn url_schemes(plist: &Path) -> Vec<String> {
    let output = Command::new("/usr/libexec/PlistBuddy")
        .args([
            "-c",
            "Print :CFBundleURLTypes",
            &plist.display().to_string(),
        ])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            line.strip_prefix("CFBundleURLSchemes:")?;
            None
        })
        .collect::<Vec<_>>()
        .into_iter()
        .chain(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| {
                    let t = line.trim().trim_matches(',').trim_matches('"');
                    if t.chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '.' || c == '-')
                        && t.contains('.')
                        || (t.len() >= 3
                            && t.chars()
                                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '+'))
                    {
                        if line.contains("CFBundle") || t == "Array" || t == "Dict" {
                            None
                        } else if t.len() >= 3 {
                            Some(t.to_string())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }),
        )
        .collect()
}

fn detect_architecture(app: &Path) -> String {
    let contents = app.join("Contents");
    if nested_chromium(&contents).is_some() {
        return "nested_chromium".into();
    }
    let frameworks = contents.join("Frameworks");
    if let Ok(entries) = fs::read_dir(&frameworks) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if name.contains("electron") {
                return "electron".into();
            }
            if name.contains("chromium embedded") {
                return "cef".into();
            }
        }
    }
    "native".into()
}

fn nested_chromium(contents: &Path) -> Option<PathBuf> {
    let helpers = contents.join("Helpers");
    find_named_dir(&helpers, "Browser Framework.framework")
        .or_else(|| find_named_dir(&helpers, "Electron Framework.framework"))
        .or_else(|| find_named_dir(&helpers, "Chromium Embedded Framework.framework"))
}

fn find_named_dir(root: &Path, needle: &str) -> Option<PathBuf> {
    fn walk(dir: &Path, needle: &str, depth: u8) -> Option<PathBuf> {
        if depth == 0 || !dir.is_dir() {
            return None;
        }
        let entries = fs::read_dir(dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.file_name().and_then(|n| n.to_str()) == Some(needle) && path.is_dir() {
                return Some(path);
            }
            if path.is_dir() {
                if let Some(found) = walk(&path, needle, depth - 1) {
                    return Some(found);
                }
            }
        }
        None
    }
    walk(root, needle, 6)
}

fn mdls_display_name(path: &Path) -> Option<String> {
    let output = Command::new("/usr/bin/mdls")
        .args(["-name", "kMDItemDisplayName", "-raw"])
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() || s == "(null)" {
        None
    } else {
        Some(s)
    }
}

fn pid_for_bundle(bundle_id: Option<&str>) -> Option<i32> {
    let bundle_id = bundle_id?;
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSWorkspace;
        let ws = NSWorkspace::sharedWorkspace();
        for app in ws.runningApplications() {
            let id = app.bundleIdentifier().map(|s| s.to_string());
            if id.as_deref() == Some(bundle_id) {
                return Some(app.processIdentifier());
            }
        }
        None
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = bundle_id;
        None
    }
}

fn listen_ports(pid: Option<i32>) -> (Vec<u16>, bool) {
    let Some(pid) = pid else {
        return (Vec::new(), false);
    };
    let output = Command::new("/usr/sbin/lsof")
        .args(["-nP", &format!("-p{pid}"), "-iTCP", "-sTCP:LISTEN"])
        .output();
    let Ok(output) = output else {
        return (Vec::new(), false);
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut ports = Vec::new();
    for line in stdout.lines() {
        if let Some(port) = parse_listen_port(line) {
            ports.push(port);
        }
    }
    ports.sort_unstable();
    ports.dedup();
    let cdp_likely = ports.iter().any(|p| cdp_port_alive(*p));
    (ports, cdp_likely)
}

fn parse_listen_port(line: &str) -> Option<u16> {
    let addr = line.split_whitespace().rev().nth(1)?;
    let port = addr.rsplit(':').next()?;
    port.parse().ok()
}

fn cdp_port_alive(port: u16) -> bool {
    let url = format!("http://127.0.0.1:{port}/json/version");
    let output = Command::new("/usr/bin/curl")
        .args(["-s", "--noproxy", "*", "-m", "1", &url])
        .output();
    match output {
        Ok(out) => {
            let body = String::from_utf8_lossy(&out.stdout);
            body.contains("webSocketDebuggerUrl") || body.contains("Browser")
        }
        Err(_) => false,
    }
}

fn duplicate_installs(bundle_id: &str) -> Vec<String> {
    let query = format!(
        "kMDItemContentType == 'com.apple.application-bundle' && kMDItemCFBundleIdentifier == '{bundle_id}'"
    );
    let output = Command::new("/usr/bin/mdfind").arg(query).output();
    let Ok(output) = output else {
        return Vec::new();
    };
    let mut paths: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| {
            !line.contains("/Application Support/")
                && !line.contains("/installed_versions/")
                && !line.contains("/Caches/")
                && !line.contains("/.Trash/")
        })
        .map(str::to_string)
        .collect();
    paths.sort();
    paths.dedup();
    if paths.len() > 1 {
        paths
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lsof_listen_line() {
        let line = "lumen-cua 1234 user  12u  IPv4 0x1  0t0  TCP 127.0.0.1:9333 (LISTEN)";
        assert_eq!(parse_listen_port(line), Some(9333));
    }

    #[test]
    fn safari_resolves_on_macos() {
        if cfg!(not(target_os = "macos")) {
            return;
        }
        let report = probe_app("Safari").expect("Safari should resolve");
        assert!(report.path.contains("Safari.app"));
        assert_eq!(report.architecture, "native");
    }
}
