use anyhow::Result;

use crate::ports::NotificationClickSupport;

pub fn check_health() -> Result<()> {
    Ok(())
}

pub fn click_action_support() -> NotificationClickSupport {
    NotificationClickSupport::Unsupported
}

pub fn notify(title: &str, body: &str, _click_url: Option<&str>) -> Result<()> {
    notify_rust::Notification::new()
        .summary(title)
        .body(body)
        .show()?;
    Ok(())
}
