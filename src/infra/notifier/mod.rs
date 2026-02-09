#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::process::{Command, ExitStatus};

use anyhow::Result;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use anyhow::{anyhow, Context};

use crate::{
    config::NotificationConfig,
    domain::events::WatchEvent,
    ports::{NotificationClickSupport, NotificationDispatchResult, NotifierPort},
};

#[cfg_attr(target_os = "macos", allow(dead_code))]
const NON_MACOS_NOOP_WARNING: &str =
    "desktop notifications are supported on macOS and WSL only; using noop notifier";
#[cfg_attr(target_os = "macos", allow(dead_code))]
const WSL_POWERSHELL_UNAVAILABLE_WARNING: &str =
    "WSL detected but powershell.exe is unavailable; using noop notifier";

#[cfg(target_os = "linux")]
const WSL_NOTIFY_POWERSHELL_SCRIPT: &str = r#"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$title = $env:GH_WATCH_NOTIFY_TITLE
$body = $env:GH_WATCH_NOTIFY_BODY
$url = $env:GH_WATCH_NOTIFY_URL
$notifyIcon = New-Object System.Windows.Forms.NotifyIcon
$notifyIcon.Icon = [System.Drawing.SystemIcons]::Information
$notifyIcon.Visible = $true
$notifyIcon.BalloonTipTitle = $title
$notifyIcon.BalloonTipText = $body
$notifyIcon.BalloonTipIcon = [System.Windows.Forms.ToolTipIcon]::Info
if (-not [string]::IsNullOrWhiteSpace($url)) {
    $notifyIcon.add_BalloonTipClicked({
        Start-Process $env:GH_WATCH_NOTIFY_URL | Out-Null
    })
}
$notifyIcon.ShowBalloonTip(10000)
Start-Sleep -Milliseconds 10500
$notifyIcon.Dispose()
"#;

pub fn build_notification_body(event: &WatchEvent, include_url: bool) -> String {
    let mut lines = vec![format!("{} by @{}", event.title, event.actor)];
    if include_url {
        lines.push(event.url.clone());
    }
    lines.join("\n")
}

fn build_notification_title(event: &WatchEvent) -> String {
    format!("{} [{}]", event.repo, event.kind)
}

fn dispatch_result(include_url: bool, click_action: bool) -> NotificationDispatchResult {
    if click_action {
        NotificationDispatchResult::DeliveredWithClickAction
    } else if include_url {
        NotificationDispatchResult::DeliveredWithBodyUrlFallback
    } else {
        NotificationDispatchResult::Delivered
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
enum DesktopBackendKind {
    MacOs,
    WslPowerShell,
    Noop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(target_os = "macos", allow(dead_code))]
struct LinuxBackendSelection {
    kind: DesktopBackendKind,
    startup_warning: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DesktopNotifier {
    backend: DesktopBackendKind,
    startup_warnings: Vec<String>,
}

impl Default for DesktopNotifier {
    fn default() -> Self {
        Self::from_notification_config(&NotificationConfig::default())
    }
}

impl DesktopNotifier {
    pub fn from_notification_config(_config: &NotificationConfig) -> Self {
        #[cfg(target_os = "macos")]
        {
            Self {
                backend: DesktopBackendKind::MacOs,
                startup_warnings: Vec::new(),
            }
        }

        #[cfg(target_os = "linux")]
        {
            let selected = detect_linux_backend();
            let startup_warnings = selected
                .startup_warning
                .into_iter()
                .collect::<Vec<String>>();

            Self {
                backend: selected.kind,
                startup_warnings,
            }
        }

        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            Self {
                backend: DesktopBackendKind::Noop,
                startup_warnings: vec![NON_MACOS_NOOP_WARNING.to_string()],
            }
        }
    }

    pub fn startup_warnings(&self) -> Vec<String> {
        self.startup_warnings.clone()
    }
}

impl NotifierPort for DesktopNotifier {
    fn check_health(&self) -> Result<()> {
        match self.backend {
            DesktopBackendKind::MacOs => {
                #[cfg(target_os = "macos")]
                {
                    check_osascript_available()
                }

                #[cfg(not(target_os = "macos"))]
                {
                    Ok(())
                }
            }
            DesktopBackendKind::WslPowerShell => Ok(()),
            DesktopBackendKind::Noop => Ok(()),
        }
    }

    fn click_action_support(&self) -> NotificationClickSupport {
        match self.backend {
            DesktopBackendKind::WslPowerShell => NotificationClickSupport::Supported,
            DesktopBackendKind::MacOs | DesktopBackendKind::Noop => {
                NotificationClickSupport::Unsupported
            }
        }
    }

    fn notify(&self, event: &WatchEvent, include_url: bool) -> Result<NotificationDispatchResult> {
        let title = build_notification_title(event);
        let body = build_notification_body(event, include_url);

        match self.backend {
            DesktopBackendKind::MacOs => {
                #[cfg(target_os = "macos")]
                {
                    notify_via_osascript(&title, &body)?;
                    Ok(dispatch_result(include_url, false))
                }

                #[cfg(not(target_os = "macos"))]
                {
                    let _ = (&title, &body);
                    Ok(dispatch_result(include_url, false))
                }
            }
            DesktopBackendKind::WslPowerShell => {
                #[cfg(target_os = "linux")]
                {
                    notify_via_powershell(&title, &body, &event.url)?;
                    Ok(dispatch_result(include_url, true))
                }

                #[cfg(not(target_os = "linux"))]
                {
                    let _ = (&title, &body);
                    Ok(dispatch_result(include_url, false))
                }
            }
            DesktopBackendKind::Noop => Ok(dispatch_result(include_url, false)),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct NoopNotifier;

impl NotifierPort for NoopNotifier {
    fn check_health(&self) -> Result<()> {
        Ok(())
    }

    fn click_action_support(&self) -> NotificationClickSupport {
        NotificationClickSupport::Unsupported
    }

    fn notify(
        &self,
        _event: &WatchEvent,
        _include_url: bool,
    ) -> Result<NotificationDispatchResult> {
        Ok(NotificationDispatchResult::Delivered)
    }
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
fn is_wsl_from_inputs(
    wsl_distro_name: Option<&str>,
    wsl_interop: Option<&str>,
    proc_hint: Option<&str>,
) -> bool {
    if wsl_distro_name.is_some_and(|value| !value.trim().is_empty()) {
        return true;
    }
    if wsl_interop.is_some_and(|value| !value.trim().is_empty()) {
        return true;
    }
    proc_hint
        .map(|value| value.to_ascii_lowercase().contains("microsoft"))
        .unwrap_or(false)
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
fn select_linux_backend(is_wsl: bool, powershell_ok: bool) -> LinuxBackendSelection {
    if !is_wsl {
        return LinuxBackendSelection {
            kind: DesktopBackendKind::Noop,
            startup_warning: Some(NON_MACOS_NOOP_WARNING.to_string()),
        };
    }

    if powershell_ok {
        LinuxBackendSelection {
            kind: DesktopBackendKind::WslPowerShell,
            startup_warning: None,
        }
    } else {
        LinuxBackendSelection {
            kind: DesktopBackendKind::Noop,
            startup_warning: Some(WSL_POWERSHELL_UNAVAILABLE_WARNING.to_string()),
        }
    }
}

#[cfg(target_os = "linux")]
fn detect_linux_backend() -> LinuxBackendSelection {
    let distro_name = std::env::var("WSL_DISTRO_NAME").ok();
    let interop = std::env::var("WSL_INTEROP").ok();
    let proc_hint = read_proc_wsl_hint();
    let is_wsl = is_wsl_from_inputs(
        distro_name.as_deref(),
        interop.as_deref(),
        proc_hint.as_deref(),
    );
    let powershell_ok = if is_wsl {
        probe_powershell_available()
    } else {
        false
    };
    select_linux_backend(is_wsl, powershell_ok)
}

#[cfg(target_os = "linux")]
fn read_proc_wsl_hint() -> Option<String> {
    let version = std::fs::read_to_string("/proc/version").ok();
    let osrelease = std::fs::read_to_string("/proc/sys/kernel/osrelease").ok();
    match (version, osrelease) {
        (Some(version), Some(osrelease)) => Some(format!("{version}\n{osrelease}")),
        (Some(version), None) => Some(version),
        (None, Some(osrelease)) => Some(osrelease),
        (None, None) => None,
    }
}

#[cfg(target_os = "linux")]
fn probe_powershell_available() -> bool {
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Write-Output ok",
        ])
        .output();
    matches!(output, Ok(out) if out.status.success())
}

#[cfg(target_os = "macos")]
fn check_osascript_available() -> Result<()> {
    let output = Command::new("osascript")
        .args(["-e", "return \"ok\""])
        .output()
        .context("failed to execute osascript")?;

    if output.status.success() {
        return Ok(());
    }

    Err(render_process_failure(
        "osascript",
        "health-check",
        &output.stdout,
        &output.stderr,
        output.status,
    ))
}

#[cfg(target_os = "macos")]
fn notify_via_osascript(title: &str, body: &str) -> Result<()> {
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        escape_apple_script_literal(body),
        escape_apple_script_literal(title)
    );

    let output = Command::new("osascript")
        .args(["-e", script.as_str()])
        .output()
        .context("failed to execute osascript")?;

    if output.status.success() {
        return Ok(());
    }

    Err(render_process_failure(
        "osascript",
        "notify",
        &output.stdout,
        &output.stderr,
        output.status,
    ))
}

#[cfg(target_os = "linux")]
fn notify_via_powershell(title: &str, body: &str, url: &str) -> Result<()> {
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            WSL_NOTIFY_POWERSHELL_SCRIPT,
        ])
        .env("GH_WATCH_NOTIFY_TITLE", title)
        .env("GH_WATCH_NOTIFY_BODY", body)
        .env("GH_WATCH_NOTIFY_URL", url)
        .output()
        .context("failed to execute powershell.exe")?;

    if output.status.success() {
        return Ok(());
    }

    Err(render_process_failure(
        "powershell.exe",
        "notify",
        &output.stdout,
        &output.stderr,
        output.status,
    ))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn render_process_failure(
    program: &str,
    operation: &str,
    stdout: &[u8],
    stderr: &[u8],
    status: ExitStatus,
) -> anyhow::Error {
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(stdout).trim().to_string();
    let detail = if !stderr.is_empty() { stderr } else { stdout };

    if detail.is_empty() {
        anyhow!("{program} {operation} failed with status {status}")
    } else {
        anyhow!("{program} {operation} failed with status {status}: {detail}")
    }
}

#[cfg(target_os = "macos")]
fn escape_apple_script_literal(raw: &str) -> String {
    raw.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    #[cfg(target_os = "macos")]
    use super::DesktopNotifier;
    use super::{
        build_notification_body, is_wsl_from_inputs, select_linux_backend, DesktopBackendKind,
    };
    #[cfg(target_os = "macos")]
    use crate::config::NotificationConfig;
    use crate::domain::events::{EventKind, WatchEvent};

    fn sample_event() -> WatchEvent {
        WatchEvent {
            event_id: "1".to_string(),
            repo: "acme/api".to_string(),
            kind: EventKind::PrCreated,
            actor: "alice".to_string(),
            title: "Add feature".to_string(),
            url: "https://example.com/pr/1".to_string(),
            created_at: Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
            source_item_id: "1".to_string(),
            subject_author: Some("alice".to_string()),
            requested_reviewer: None,
            mentions: Vec::new(),
        }
    }

    #[test]
    fn notification_body_contains_url_when_requested() {
        let body = build_notification_body(&sample_event(), true);
        assert!(body.contains("https://example.com/pr/1"));
    }

    #[test]
    fn wsl_detection_true_when_wsl_distro_name_exists() {
        assert!(is_wsl_from_inputs(
            Some("Ubuntu"),
            None,
            Some("Linux version 6.6.87.2-microsoft-standard-WSL2")
        ));
    }

    #[test]
    fn wsl_detection_true_when_wsl_interop_exists() {
        assert!(is_wsl_from_inputs(
            None,
            Some("/run/WSL/123_interop"),
            Some("Linux version 6.6.87.2-microsoft-standard-WSL2")
        ));
    }

    #[test]
    fn wsl_detection_true_when_proc_contains_microsoft() {
        assert!(is_wsl_from_inputs(
            None,
            None,
            Some("Linux version 6.6.87.2-microsoft-standard-WSL2")
        ));
    }

    #[test]
    fn wsl_detection_false_when_all_signals_absent() {
        assert!(!is_wsl_from_inputs(
            None,
            None,
            Some("Linux version 6.6.87.2-generic")
        ));
    }

    #[test]
    fn linux_backend_non_wsl_falls_back_to_noop_with_warning() {
        let selected = select_linux_backend(false, false);
        assert_eq!(selected.kind, DesktopBackendKind::Noop);
        let warning = selected.startup_warning.expect("warning should exist");
        assert!(warning.contains("macOS and WSL"));
    }

    #[test]
    fn linux_backend_wsl_with_powershell_selects_wsl_backend() {
        let selected = select_linux_backend(true, true);
        assert_eq!(selected.kind, DesktopBackendKind::WslPowerShell);
        assert!(selected.startup_warning.is_none());
    }

    #[test]
    fn linux_backend_wsl_without_powershell_falls_back_to_noop_with_warning() {
        let selected = select_linux_backend(true, false);
        assert_eq!(selected.kind, DesktopBackendKind::Noop);
        let warning = selected.startup_warning.expect("warning should exist");
        assert!(warning.contains("powershell.exe"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn startup_warnings_macos_are_empty() {
        let notifier = DesktopNotifier::from_notification_config(&NotificationConfig::default());
        assert!(notifier.startup_warnings().is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn escape_apple_script_literal_escapes_quotes_and_newlines() {
        let escaped = super::escape_apple_script_literal("a\n\"b\"");
        assert_eq!(escaped, "a\\n\\\"b\\\"");
    }
}
