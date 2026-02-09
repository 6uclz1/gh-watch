use std::{
    env, fs,
    process::{Command, Output},
};

use anyhow::{anyhow, Context, Result};

use super::PlatformNotificationOptions;
use crate::ports::NotificationClickSupport;

pub const DEFAULT_WINDOWS_APP_ID: &str =
    "{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\WindowsPowerShell\\v1.0\\powershell.exe";

const POWERSHELL_BIN: &str = "powershell.exe";
const POWERSHELL_HEALTHCHECK_SCRIPT: &str = "$null = $PSVersionTable.PSVersion";
const POWERSHELL_TOAST_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop';
try {
    [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null;
    [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] > $null;
    $appId = $env:GH_WATCH_TOAST_APP_ID;
    $title = [System.Security.SecurityElement]::Escape($env:GH_WATCH_TOAST_TITLE);
    $body = [System.Security.SecurityElement]::Escape($env:GH_WATCH_TOAST_BODY);
    $xml = New-Object Windows.Data.Xml.Dom.XmlDocument;
    $xml.LoadXml("<toast><visual><binding template='ToastGeneric'><text>$title</text><text>$body</text></binding></visual></toast>");
    $toast = [Windows.UI.Notifications.ToastNotification]::new($xml);
    [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($appId).Show($toast);
    exit 0;
}
catch {
    $message = $_.Exception.Message;
    if ([string]::IsNullOrWhiteSpace($message)) {
        $message = $_ | Out-String;
    }
    [Console]::Error.WriteLine($message.Trim());
    exit 1;
}
"#;

pub fn effective_wsl_app_id(configured: Option<&str>) -> String {
    configured
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_WINDOWS_APP_ID)
        .to_string()
}

fn is_wsl() -> bool {
    let wsl_distro_name = env::var("WSL_DISTRO_NAME").ok();
    let wsl_interop = env::var("WSL_INTEROP").ok();
    let proc_version = fs::read_to_string("/proc/version").ok();

    is_wsl_from_signals(
        wsl_distro_name.as_deref(),
        wsl_interop.as_deref(),
        proc_version.as_deref(),
    )
}

fn is_wsl_from_signals(
    wsl_distro_name: Option<&str>,
    wsl_interop: Option<&str>,
    proc_version: Option<&str>,
) -> bool {
    if has_signal(wsl_distro_name) || has_signal(wsl_interop) {
        return true;
    }

    proc_version
        .map(|value| value.to_ascii_lowercase().contains("microsoft"))
        .unwrap_or(false)
}

fn has_signal(value: Option<&str>) -> bool {
    value
        .map(str::trim)
        .map(|trimmed| !trimmed.is_empty())
        .unwrap_or(false)
}

fn check_health_with_wsl_override(_options: &PlatformNotificationOptions, wsl: bool) -> Result<()> {
    if !wsl {
        return Ok(());
    }

    ensure_powershell_available()
}

pub fn check_health(options: &PlatformNotificationOptions) -> Result<()> {
    check_health_with_wsl_override(options, is_wsl())
}

pub fn click_action_support() -> NotificationClickSupport {
    NotificationClickSupport::Unsupported
}

pub fn notify(
    title: &str,
    body: &str,
    _click_url: Option<&str>,
    options: &PlatformNotificationOptions,
) -> Result<()> {
    notify_with_wsl_override(title, body, options, is_wsl())
}

fn notify_with_wsl_override(
    title: &str,
    body: &str,
    options: &PlatformNotificationOptions,
    wsl: bool,
) -> Result<()> {
    if wsl {
        return notify_via_windows_toast(title, body, options);
    }

    notify_rust::Notification::new()
        .summary(title)
        .body(body)
        .show()?;
    Ok(())
}

fn ensure_powershell_available() -> Result<()> {
    let output = Command::new(POWERSHELL_BIN)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            POWERSHELL_HEALTHCHECK_SCRIPT,
        ])
        .output()
        .with_context(|| format!("failed to execute {POWERSHELL_BIN} for WSL notifications"))?;

    if output.status.success() {
        return Ok(());
    }

    Err(format_powershell_failure("health check", &output))
}

fn notify_via_windows_toast(
    title: &str,
    body: &str,
    options: &PlatformNotificationOptions,
) -> Result<()> {
    let app_id = effective_wsl_app_id(options.wsl_windows_app_id.as_deref());
    let output = Command::new(POWERSHELL_BIN)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            POWERSHELL_TOAST_SCRIPT,
        ])
        .env("GH_WATCH_TOAST_APP_ID", &app_id)
        .env("GH_WATCH_TOAST_TITLE", title)
        .env("GH_WATCH_TOAST_BODY", body)
        .output()
        .with_context(|| format!("failed to execute {POWERSHELL_BIN} for WSL notifications"))?;

    if output.status.success() {
        return Ok(());
    }

    Err(format_powershell_failure("toast command", &output))
}

fn format_powershell_failure(operation: &str, output: &Output) -> anyhow::Error {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if !stderr.is_empty() {
        Some(stderr)
    } else if !stdout.is_empty() {
        Some(stdout)
    } else {
        None
    };

    if let Some(detail) = detail {
        anyhow!(
            "{POWERSHELL_BIN} {operation} failed with status {}: {}",
            output.status,
            detail
        )
    } else {
        anyhow!(
            "{POWERSHELL_BIN} {operation} failed with status {}",
            output.status
        )
    }
}

#[cfg(test)]
mod tests {
    use std::{
        env, fs,
        io::Write,
        path::{Path, PathBuf},
        sync::{Mutex, OnceLock},
    };

    use tempfile::tempdir;

    use super::{
        check_health_with_wsl_override, effective_wsl_app_id, is_wsl_from_signals,
        notify_with_wsl_override, DEFAULT_WINDOWS_APP_ID,
    };
    use crate::infra::notifier::PlatformNotificationOptions;

    #[test]
    fn wsl_detection_is_true_when_distro_env_is_set() {
        assert!(is_wsl_from_signals(Some("Ubuntu"), None, None));
    }

    #[test]
    fn wsl_detection_is_true_when_interop_env_is_set() {
        assert!(is_wsl_from_signals(
            None,
            Some("/run/WSL/123_interop"),
            None
        ));
    }

    #[test]
    fn wsl_detection_is_true_when_proc_version_mentions_microsoft() {
        assert!(is_wsl_from_signals(
            None,
            None,
            Some("Linux version 5.15.167.4-microsoft-standard-WSL2")
        ));
    }

    #[test]
    fn wsl_detection_is_false_without_any_signals() {
        assert!(!is_wsl_from_signals(
            None,
            None,
            Some("Linux version 6.8.0-generic")
        ));
    }

    #[test]
    fn effective_wsl_app_id_uses_default_when_missing() {
        assert_eq!(effective_wsl_app_id(None), DEFAULT_WINDOWS_APP_ID);
    }

    #[test]
    fn check_health_in_wsl_requires_powershell() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let path_guard = EnvVarGuard::set("PATH", "");

        let options = PlatformNotificationOptions::default();
        let result = check_health_with_wsl_override(&options, true);

        drop(path_guard);
        assert!(result.is_err());
    }

    #[test]
    fn check_health_in_wsl_succeeds_when_powershell_exists() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempdir().unwrap();
        let script_path = write_stub_powershell(
            dir.path(),
            r#"#!/usr/bin/env bash
set -euo pipefail
exit 0
"#,
        );
        assert!(script_path.exists());

        let original_path = env::var("PATH").unwrap_or_default();
        let joined_path = join_path_front(dir.path(), &original_path);
        let path_guard = EnvVarGuard::set("PATH", &joined_path);

        let options = PlatformNotificationOptions::default();
        let result = check_health_with_wsl_override(&options, true);

        drop(path_guard);
        assert!(result.is_ok());
    }

    #[test]
    fn check_health_in_wsl_reports_stdout_when_stderr_is_empty() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempdir().unwrap();
        let script_path = write_stub_powershell(
            dir.path(),
            r#"#!/usr/bin/env bash
set -euo pipefail
echo "health stdout detail"
exit 1
"#,
        );
        assert!(script_path.exists());

        let original_path = env::var("PATH").unwrap_or_default();
        let joined_path = join_path_front(dir.path(), &original_path);
        let path_guard = EnvVarGuard::set("PATH", &joined_path);

        let options = PlatformNotificationOptions::default();
        let result = check_health_with_wsl_override(&options, true);

        drop(path_guard);
        let err = result.expect_err("health check should fail");
        let message = err.to_string();
        assert!(message.contains("health stdout detail"));
    }

    #[test]
    fn wsl_notify_uses_powershell_and_prefers_wsl_app_id() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempdir().unwrap();
        let output_path = dir.path().join("toast_env.txt");
        let script_path = write_stub_powershell(
            dir.path(),
            r#"#!/usr/bin/env bash
set -euo pipefail
{
  printf '%s\n' "${GH_WATCH_TOAST_APP_ID}"
  printf '%s\n' "${GH_WATCH_TOAST_TITLE}"
  printf '%s\n' "${GH_WATCH_TOAST_BODY}"
} > "${GH_WATCH_TEST_OUTPUT}"
"#,
        );
        assert!(script_path.exists());

        let original_path = env::var("PATH").unwrap_or_default();
        let joined_path = join_path_front(dir.path(), &original_path);
        let path_guard = EnvVarGuard::set("PATH", &joined_path);
        let output_guard =
            EnvVarGuard::set("GH_WATCH_TEST_OUTPUT", &output_path.display().to_string());

        let options = PlatformNotificationOptions {
            macos_bundle_id: None,
            windows_app_id: Some("com.example.ShouldBeIgnored".to_string()),
            wsl_windows_app_id: Some("com.example.WslPreferred".to_string()),
        };

        let result = notify_with_wsl_override("T", "B", &options, true);

        drop(output_guard);
        drop(path_guard);
        assert!(result.is_ok());

        let output = fs::read_to_string(output_path).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines, vec!["com.example.WslPreferred", "T", "B"]);
    }

    #[test]
    fn wsl_notify_returns_error_when_powershell_fails() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempdir().unwrap();
        let script_path = write_stub_powershell(
            dir.path(),
            r#"#!/usr/bin/env bash
set -euo pipefail
echo "toast failed" >&2
exit 23
"#,
        );
        assert!(script_path.exists());

        let original_path = env::var("PATH").unwrap_or_default();
        let joined_path = join_path_front(dir.path(), &original_path);
        let path_guard = EnvVarGuard::set("PATH", &joined_path);

        let options = PlatformNotificationOptions {
            macos_bundle_id: None,
            windows_app_id: None,
            wsl_windows_app_id: None,
        };
        let result = notify_with_wsl_override("T", "B", &options, true);

        drop(path_guard);
        assert!(result.is_err());
    }

    #[test]
    fn wsl_notify_reports_stdout_when_stderr_is_empty() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempdir().unwrap();
        let script_path = write_stub_powershell(
            dir.path(),
            r#"#!/usr/bin/env bash
set -euo pipefail
echo "toast API unavailable"
exit 1
"#,
        );
        assert!(script_path.exists());

        let original_path = env::var("PATH").unwrap_or_default();
        let joined_path = join_path_front(dir.path(), &original_path);
        let path_guard = EnvVarGuard::set("PATH", &joined_path);

        let options = PlatformNotificationOptions {
            macos_bundle_id: None,
            windows_app_id: None,
            wsl_windows_app_id: None,
        };
        let result = notify_with_wsl_override("T", "B", &options, true);

        drop(path_guard);
        let err = result.expect_err("toast should fail");
        let message = err.to_string();
        assert!(message.contains("toast API unavailable"));
    }

    #[test]
    fn wsl_notify_prefers_stderr_over_stdout() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempdir().unwrap();
        let script_path = write_stub_powershell(
            dir.path(),
            r#"#!/usr/bin/env bash
set -euo pipefail
echo "stdout detail"
echo "stderr detail" >&2
exit 1
"#,
        );
        assert!(script_path.exists());

        let original_path = env::var("PATH").unwrap_or_default();
        let joined_path = join_path_front(dir.path(), &original_path);
        let path_guard = EnvVarGuard::set("PATH", &joined_path);

        let options = PlatformNotificationOptions {
            macos_bundle_id: None,
            windows_app_id: None,
            wsl_windows_app_id: None,
        };
        let result = notify_with_wsl_override("T", "B", &options, true);

        drop(path_guard);
        let err = result.expect_err("toast should fail");
        let message = err.to_string();
        assert!(message.contains("stderr detail"));
        assert!(!message.contains("stdout detail"));
    }

    fn write_stub_powershell(dir: &Path, script: &str) -> PathBuf {
        let path = dir.join("powershell.exe");
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(script.as_bytes()).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions).unwrap();
        }

        path
    }

    fn join_path_front(path: &Path, existing: &str) -> String {
        if existing.is_empty() {
            path.display().to_string()
        } else {
            format!("{}:{existing}", path.display())
        }
    }

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct EnvVarGuard {
        key: String,
        old: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &str, value: &str) -> Self {
            let old = env::var(key).ok();
            env::set_var(key, value);
            Self {
                key: key.to_string(),
                old,
            }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(old) = &self.old {
                env::set_var(&self.key, old);
            } else {
                env::remove_var(&self.key);
            }
        }
    }
}
