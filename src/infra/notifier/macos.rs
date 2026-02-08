use anyhow::{Context, Result};

use super::PlatformNotificationOptions;
use crate::ports::NotificationClickSupport;

pub const DEFAULT_BUNDLE_ID: &str = "com.apple.Terminal";

pub fn effective_bundle_id(configured: Option<&str>) -> String {
    configured
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_BUNDLE_ID)
        .to_string()
}

fn ensure_application(options: &PlatformNotificationOptions) -> Result<()> {
    let bundle_id = effective_bundle_id(options.macos_bundle_id.as_deref());
    match mac_notification_sys::set_application(&bundle_id) {
        Ok(()) => Ok(()),
        Err(mac_notification_sys::error::Error::Application(
            mac_notification_sys::error::ApplicationError::AlreadySet(_),
        )) => Ok(()),
        Err(err) => Err(err)
            .with_context(|| format!("failed to set macOS notification bundle id: {bundle_id}")),
    }
}

pub fn check_health(options: &PlatformNotificationOptions) -> Result<()> {
    ensure_application(options)
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
    ensure_application(options)?;
    mac_notification_sys::send_notification(title, None, body, None)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{effective_bundle_id, DEFAULT_BUNDLE_ID};

    #[test]
    fn effective_bundle_id_uses_default_when_missing() {
        assert_eq!(effective_bundle_id(None), DEFAULT_BUNDLE_ID);
    }

    #[test]
    fn effective_bundle_id_uses_default_when_blank() {
        assert_eq!(effective_bundle_id(Some("   ")), DEFAULT_BUNDLE_ID);
    }

    #[test]
    fn effective_bundle_id_prefers_configured_value() {
        assert_eq!(
            effective_bundle_id(Some("com.example.CustomMacApp")),
            "com.example.CustomMacApp"
        );
    }
}
