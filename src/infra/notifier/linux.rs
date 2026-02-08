use anyhow::Result;

use super::PlatformNotificationOptions;
use crate::ports::NotificationClickSupport;

pub fn check_health(_options: &PlatformNotificationOptions) -> Result<()> {
    Ok(())
}

pub fn click_action_support() -> NotificationClickSupport {
    NotificationClickSupport::Unsupported
}

pub fn notify(
    title: &str,
    body: &str,
    _click_url: Option<&str>,
    _options: &PlatformNotificationOptions,
) -> Result<()> {
    notify_rust::Notification::new()
        .summary(title)
        .body(body)
        .show()?;
    Ok(())
}
