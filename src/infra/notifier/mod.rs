use std::process::Command;

use anyhow::Result;
#[cfg(target_os = "macos")]
use anyhow::{anyhow, Context};

use crate::{
    config::NotificationConfig,
    domain::events::WatchEvent,
    ports::{NotificationClickSupport, NotificationDispatchResult, NotifierPort},
};

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

fn dispatch_result(include_url: bool) -> NotificationDispatchResult {
    if include_url {
        NotificationDispatchResult::DeliveredWithBodyUrlFallback
    } else {
        NotificationDispatchResult::Delivered
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DesktopNotifier;

impl DesktopNotifier {
    pub fn from_notification_config(_config: &NotificationConfig) -> Self {
        Self
    }

    pub fn startup_warnings(&self) -> Vec<String> {
        #[cfg(target_os = "macos")]
        {
            Vec::new()
        }

        #[cfg(not(target_os = "macos"))]
        {
            vec![
                "osascript notifications are only supported on macOS; using noop notifier"
                    .to_string(),
            ]
        }
    }
}

impl NotifierPort for DesktopNotifier {
    fn check_health(&self) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            check_osascript_available()
        }

        #[cfg(not(target_os = "macos"))]
        {
            Ok(())
        }
    }

    fn click_action_support(&self) -> NotificationClickSupport {
        NotificationClickSupport::Unsupported
    }

    fn notify(&self, event: &WatchEvent, include_url: bool) -> Result<NotificationDispatchResult> {
        let title = build_notification_title(event);
        let body = build_notification_body(event, include_url);

        #[cfg(target_os = "macos")]
        {
            notify_via_osascript(&title, &body)?;
            Ok(dispatch_result(include_url))
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = (title, body);
            Ok(dispatch_result(include_url))
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

#[cfg(target_os = "macos")]
fn check_osascript_available() -> Result<()> {
    let output = Command::new("osascript")
        .args(["-e", "return \"ok\""])
        .output()
        .context("failed to execute osascript")?;

    if output.status.success() {
        return Ok(());
    }

    Err(render_osascript_failure(
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

    Err(render_osascript_failure(
        "notify",
        &output.stdout,
        &output.stderr,
        output.status,
    ))
}

#[cfg(target_os = "macos")]
fn render_osascript_failure(
    operation: &str,
    stdout: &[u8],
    stderr: &[u8],
    status: std::process::ExitStatus,
) -> anyhow::Error {
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(stdout).trim().to_string();
    let detail = if !stderr.is_empty() { stderr } else { stdout };

    if detail.is_empty() {
        anyhow!("osascript {} failed with status {}", operation, status)
    } else {
        anyhow!(
            "osascript {} failed with status {}: {}",
            operation,
            status,
            detail
        )
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

    use super::{build_notification_body, DesktopNotifier};
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

    #[cfg(target_os = "macos")]
    #[test]
    fn startup_warnings_macos_are_empty() {
        let notifier = DesktopNotifier;
        assert!(notifier.startup_warnings().is_empty());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn startup_warnings_non_macos_warns_noop() {
        let notifier = DesktopNotifier;
        let warnings = notifier.startup_warnings();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("only supported on macOS"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn escape_apple_script_literal_escapes_quotes_and_newlines() {
        let escaped = super::escape_apple_script_literal("a\n\"b\"");
        assert_eq!(escaped, "a\\n\\\"b\\\"");
    }
}
