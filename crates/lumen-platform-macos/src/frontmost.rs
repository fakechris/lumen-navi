//! Frontmost application probe (cheap signal for screenshot.v1 payload).

use async_trait::async_trait;
use lumen_platform::{FrontmostApp, FrontmostAppProbe, PlatformError};

pub struct MacFrontmost;

#[async_trait]
impl FrontmostAppProbe for MacFrontmost {
    async fn frontmost(&self) -> Result<Option<FrontmostApp>, PlatformError> {
        Ok(frontmost_app())
    }
}

pub fn frontmost_app() -> Option<FrontmostApp> {
    frontmost_native().or_else(frontmost_osascript)
}

#[cfg(target_os = "macos")]
fn frontmost_native() -> Option<FrontmostApp> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::NSString;

    let ws = NSWorkspace::sharedWorkspace();
    let app = ws.frontmostApplication()?;
    let app_name = app
        .localizedName()
        .map(|s: objc2::rc::Retained<NSString>| s.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    let bundle_id = app
        .bundleIdentifier()
        .map(|s: objc2::rc::Retained<NSString>| s.to_string())
        .filter(|s| !s.is_empty());
    Some(FrontmostApp {
        app_name,
        bundle_id,
        window_title: None,
    })
}

#[cfg(not(target_os = "macos"))]
fn frontmost_native() -> Option<FrontmostApp> {
    None
}

fn frontmost_osascript() -> Option<FrontmostApp> {
    #[cfg(target_os = "macos")]
    {
        let script = r#"
tell application "System Events"
  set p to first application process whose frontmost is true
  set n to name of p
  set b to ""
  try
    set b to bundle identifier of p
  end try
  return n & linefeed & b
end tell
"#;
        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&output.stdout);
        let mut lines = s.lines();
        let name = lines.next().map(str::trim).filter(|x| !x.is_empty())?;
        let bundle = lines
            .next()
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(|x| x.to_string());
        Some(FrontmostApp {
            app_name: name.to_string(),
            bundle_id: bundle,
            window_title: None,
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}
