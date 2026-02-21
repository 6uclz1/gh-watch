#[cfg(target_os = "linux")]
use std::process::Command;

#[cfg(target_os = "linux")]
use anyhow::{Context, Result};

#[cfg(target_os = "linux")]
use super::process_error::render_process_failure;

#[cfg(target_os = "linux")]
pub(super) const WSL_NOTIFY_BURNTTOAST_SCRIPT: &str = r#"
Import-Module BurntToast -ErrorAction Stop
$title = $env:GH_WATCH_NOTIFY_TITLE
$body = $env:GH_WATCH_NOTIFY_BODY
New-BurntToastNotification -Text $title, $body | Out-Null
"#;

#[cfg(target_os = "linux")]
pub(super) fn read_proc_wsl_hint() -> Option<String> {
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
pub(super) fn probe_burnttoast_available() -> bool {
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Import-Module BurntToast -ErrorAction Stop; Get-Command New-BurntToastNotification -ErrorAction Stop | Out-Null; Write-Output ok",
        ])
        .output();
    matches!(output, Ok(out) if out.status.success())
}

#[cfg(target_os = "linux")]
pub(super) fn notify_via_burnttoast(title: &str, body: &str) -> Result<()> {
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            WSL_NOTIFY_BURNTTOAST_SCRIPT,
        ])
        .env("GH_WATCH_NOTIFY_TITLE", title)
        .env("GH_WATCH_NOTIFY_BODY", body)
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
